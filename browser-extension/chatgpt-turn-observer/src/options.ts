import { loadSettings, saveSettings } from './settings';
import { normalizeObserverBaseUrl, validateObserverStatusPayload } from './observer-protocol';
import type { BridgeMode } from './types';

document.addEventListener('DOMContentLoaded', async () => {
  const bridgeModeSelect = document.getElementById('bridge-mode') as HTMLSelectElement;
  const localBaseUrlInput = document.getElementById('local-base-url') as HTMLInputElement;
  const remoteBaseUrlInput = document.getElementById('remote-base-url') as HTMLInputElement;
  const bridgeTokenInput = document.getElementById('bridge-token') as HTMLInputElement;
  const toggleTokenBtn = document.getElementById('toggle-token-btn') as HTMLButtonElement;
  const saveBtn = document.getElementById('save-btn') as HTMLButtonElement;
  const testBtn = document.getElementById('test-btn') as HTMLButtonElement;
  const saveStatus = document.getElementById('save-status') as HTMLSpanElement;
  const testResultBox = document.getElementById('test-result-box') as HTMLDivElement;
  const testResultTitle = document.getElementById('test-result-title') as HTMLDivElement;
  const testResultDetail = document.getElementById('test-result-detail') as HTMLDivElement;

  // 1. 初始化加载
  const settings = await loadSettings();
  bridgeModeSelect.value = settings.bridgeMode;
  localBaseUrlInput.value = settings.localBaseUrl;
  remoteBaseUrlInput.value = settings.remoteBaseUrl;
  bridgeTokenInput.value = settings.bridgeToken;

  // 2. Token 显隐切换
  toggleTokenBtn.addEventListener('click', () => {
    if (bridgeTokenInput.type === 'password') {
      bridgeTokenInput.type = 'text';
      toggleTokenBtn.textContent = '隐藏';
    } else {
      bridgeTokenInput.type = 'password';
      toggleTokenBtn.textContent = '显示';
    }
  });

  // 3. 保存配置
  saveBtn.addEventListener('click', async () => {
    await saveSettings({
      bridgeMode: bridgeModeSelect.value as BridgeMode,
      localBaseUrl: localBaseUrlInput.value.trim(),
      remoteBaseUrl: remoteBaseUrlInput.value.trim(),
      bridgeToken: bridgeTokenInput.value.trim(),
    });
    saveStatus.textContent = '配置已保存！';
    setTimeout(() => {
      saveStatus.textContent = '';
    }, 2500);
  });

  // 4. 测试连接（统一使用 GET /internal/chatgpt-turn-observer/status）
  async function testTarget(name: string, baseUrl: string, token: string): Promise<{ ok: boolean; message: string }> {
    if (!baseUrl) {
      return { ok: false, message: `${name}: 未配置地址` };
    }
    const normalizedBaseUrl = normalizeObserverBaseUrl(baseUrl);
    if (!normalizedBaseUrl) {
      return { ok: false, message: `${name}: 地址为空` };
    }
    const cleanUrl = `${normalizedBaseUrl}/internal/chatgpt-turn-observer/status`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 4000);

    try {
      const resp = await fetch(cleanUrl, {
        method: 'GET',
        headers: {
          Authorization: `Bearer ${token.trim()}`,
        },
        signal: controller.signal,
      });
      clearTimeout(timer);

      if (resp.ok) {
        let jsonPayload: unknown = null;
        try {
          jsonPayload = await resp.json();
        } catch {
          return { ok: false, message: `${name}: 状态响应不是有效 JSON` };
        }
        const statusCheck = validateObserverStatusPayload(jsonPayload);
        if (!statusCheck.ok) {
          return { ok: false, message: `${name}: 不是有效的 ChatGPT Turn Observer (${statusCheck.error})` };
        }
        return {
          ok: true,
          message: `${name}: 连接成功！(HTTP 200, status=${JSON.stringify(jsonPayload)})`,
        };
      } else if (resp.status === 401 || resp.status === 403) {
        return {
          ok: false,
          message: `${name}: 401 认证失败，请核对 Browser Bridge Token`,
        };
      } else {
        return {
          ok: false,
          message: `${name}: 服务响应错误 (HTTP ${resp.status})`,
        };
      }
    } catch (err: unknown) {
      clearTimeout(timer);
      const isTimeout = err instanceof Error && err.name === 'AbortError';
      return {
        ok: false,
        message: `${name}: 连接失败 (${isTimeout ? '请求超时 4s' : err instanceof Error ? err.message : '网络错误'})`,
      };
    }
  }

  testBtn.addEventListener('click', async () => {
    const mode = bridgeModeSelect.value as BridgeMode;
    const localUrl = localBaseUrlInput.value.trim();
    const remoteUrl = remoteBaseUrlInput.value.trim();
    const token = bridgeTokenInput.value.trim();

    testResultBox.className = 'test-result-box';
    testResultTitle.textContent = '正在测试连接…';
    testResultDetail.textContent = '请稍候…';

    const results: string[] = [];
    let allOk = true;

    if (mode === 'local' || mode === 'auto') {
      const res = await testTarget('Local', localUrl, token);
      results.push(res.message);
      if (!res.ok && mode === 'local') allOk = false;
    }

    if (mode === 'remote' || (mode === 'auto' && remoteUrl)) {
      const res = await testTarget('Remote', remoteUrl, token);
      results.push(res.message);
      if (!res.ok && mode === 'remote') allOk = false;
    }

    if (mode === 'auto') {
      // Auto 模式下只要 Local 或 Remote 成功一个即视为可用
      const hasSuccess = results.some((r) => r.includes('连接成功'));
      allOk = hasSuccess;
    }

    testResultBox.className = `test-result-box ${allOk ? 'success' : 'error'}`;
    testResultTitle.textContent = allOk ? '✅ 测试通过' : '❌ 测试未通过';
    testResultDetail.textContent = '';
    for (const r of results) {
      const div = document.createElement('div');
      div.textContent = r;
      testResultDetail.appendChild(div);
    }
  });
});
