import { formatModelDisplayName } from './parsers';
import { loadSettings, saveSettings } from './settings';
import type { BridgeMode, ObserverSettings, TabTurnState } from './types';

export class TurnObserverOverlay {
  private container: HTMLDivElement | null = null;
  private modalContainer: HTMLDivElement | null = null;
  private isCollapsed = false;
  private position = { x: 0, y: 0 };
  private onPositionChange?: (pos: { x: number; y: number }) => void;
  private onCollapsedChange?: (collapsed: boolean) => void;
  public onSettingsSaved?: () => void;

  // 计时器状态
  private timerInterval: number | null = null;
  private timerSeconds = 0;
  private isTimerRunning = false;

  constructor(
    initialPos: { x: number; y: number } | null,
    initialCollapsed: boolean,
    onPositionChange?: (pos: { x: number; y: number }) => void,
    onCollapsedChange?: (collapsed: boolean) => void,
    onSettingsSaved?: () => void
  ) {
    this.isCollapsed = initialCollapsed;
    this.onPositionChange = onPositionChange;
    this.onCollapsedChange = onCollapsedChange;
    this.onSettingsSaved = onSettingsSaved;

    // 默认放在右上角
    const defaultX = Math.max(20, window.innerWidth - 240);
    const defaultY = 80;
    this.position = initialPos || { x: defaultX, y: defaultY };
    this.clampPosition();

    this.mount();
    window.addEventListener('resize', this.handleResize);
  }

  private handleResize = () => {
    this.clampPosition();
    this.updatePositionStyle();
  };

  private clampPosition() {
    const maxX = Math.max(0, window.innerWidth - (this.isCollapsed ? 160 : 220));
    const maxY = Math.max(0, window.innerHeight - (this.isCollapsed ? 40 : 160));
    this.position.x = Math.max(10, Math.min(this.position.x, maxX));
    this.position.y = Math.max(10, Math.min(this.position.y, maxY));
  }

  private mount() {
    if (this.container) return;

    this.container = document.createElement('div');
    this.container.className = 'ct-turn-observer-root';
    this.updatePositionStyle();

    document.body.appendChild(this.container);
    this.render();
  }

  private updatePositionStyle() {
    if (!this.container) return;
    this.container.style.left = `${this.position.x}px`;
    this.container.style.top = `${this.position.y}px`;
  }

  public updateState(state: TabTurnState) {
    this.manageTimer(state);
    this.render(state);
  }

  private manageTimer(state: TabTurnState) {
    if (state.startedAt && (state.state === 'turn_starting' || state.state === 'active')) {
      if (!this.isTimerRunning) {
        this.isTimerRunning = true;
        this.startTimerLoop(state.startedAt);
      }
    } else if (state.state === 'completed' || state.state === 'stream_idle') {
      // 停止计时
      if (this.isTimerRunning && state.state === 'completed') {
        this.stopTimerLoop();
      }
    } else if (state.state === 'idle') {
      this.stopTimerLoop();
      this.timerSeconds = 0;
    }
  }

  private startTimerLoop(startedAt: number) {
    if (this.timerInterval !== null) {
      clearInterval(this.timerInterval);
    }
    const update = () => {
      const now = Date.now();
      this.timerSeconds = Math.max(0, Math.floor((now - startedAt) / 1000));
      this.updateTimerDisplay();
    };
    update();
    this.timerInterval = window.setInterval(update, 1000);
  }

  private stopTimerLoop() {
    if (this.timerInterval !== null) {
      clearInterval(this.timerInterval);
      this.timerInterval = null;
    }
    this.isTimerRunning = false;
  }

  private formatDuration(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
  }

  private updateTimerDisplay() {
    if (!this.container) return;
    const timeEl = this.container.querySelector('.ct-timer-val');
    if (timeEl) {
      timeEl.textContent = this.formatDuration(this.timerSeconds);
    }
    const miniTimeEl = this.container.querySelector('.ct-mini-timer');
    if (miniTimeEl) {
      miniTimeEl.textContent = this.formatDuration(this.timerSeconds);
    }
  }

  private render(state?: TabTurnState) {
    if (!this.container) return;

    const actualModelText = state?.actualModel
      ? formatModelDisplayName(state.actualModel)
      : null;
    const requestedModelText = state?.requestedModel
      ? formatModelDisplayName(state.requestedModel)
      : null;

    let modelRowLabel = '模型：';
    let modelRowValue = '—';
    if (actualModelText) {
      modelRowLabel = '模型：';
      modelRowValue = actualModelText;
    } else if (requestedModelText) {
      modelRowLabel = '请求：';
      modelRowValue = `${requestedModelText} (实际: 未知)`;
    } else if (state?.state === 'active' || state?.state === 'turn_starting') {
      modelRowLabel = '模型：';
      modelRowValue = '检测中…';
    } else {
      modelRowLabel = '模型：';
      modelRowValue = '—';
    }

    let statusClass = 'idle';
    let statusLabel = '待同步';
    if (state?.bridgeStatus === 'synced') {
      statusClass = 'synced';
      statusLabel = '已同步';
    } else if (state?.bridgeStatus === 'sending') {
      statusClass = 'sending';
      statusLabel = '发送中';
    } else if (state?.bridgeStatus === 'failed') {
      statusClass = 'failed';
      statusLabel = '同步失败';
    } else if (state?.bridgeStatus === 'not_configured') {
      statusClass = 'failed';
      statusLabel = '未配置 Token';
    } else if (state?.bridgeStatus === 'idle') {
      statusClass = 'idle';
      statusLabel = '待同步';
    }

    const durationText = this.formatDuration(this.timerSeconds);

    if (this.isCollapsed) {
      this.container.innerHTML = `
        <div class="ct-turn-observer-mini" id="ct-drag-handle">
          <span class="ct-turn-observer-logo-dot"></span>
          <span>CT</span>
          <span>·</span>
          <span>${actualModelText || requestedModelText || '—'}</span>
          <span>·</span>
          <span class="ct-mini-timer">${durationText}</span>
          <span class="ct-turn-observer-status-dot ${statusClass}"></span>
          <button type="button" class="ct-turn-observer-toggle-btn" id="ct-expand-btn" title="展开">＋</button>
        </div>
      `;
    } else {
      this.container.innerHTML = `
        <div class="ct-turn-observer-card">
          <div class="ct-turn-observer-header" id="ct-drag-handle">
            <div class="ct-turn-observer-title">
              <span class="ct-turn-observer-logo-dot"></span>
              <span>Coding Tools</span>
            </div>
            <div class="ct-turn-observer-actions">
              <button type="button" class="ct-turn-observer-toggle-btn" id="ct-settings-btn" title="配置 (打开设置)">⚙</button>
              <button type="button" class="ct-turn-observer-toggle-btn" id="ct-collapse-btn" title="折叠">－</button>
            </div>
          </div>
          <div class="ct-turn-observer-row">
            <span class="ct-turn-observer-label">${modelRowLabel}</span>
            <span class="ct-turn-observer-value" title="${modelRowValue}">${modelRowValue}</span>
          </div>
          <div class="ct-turn-observer-row">
            <span class="ct-turn-observer-label">本轮：</span>
            <span class="ct-turn-observer-value ct-timer-val">${durationText}</span>
          </div>
          <div class="ct-turn-observer-row">
            <span class="ct-turn-observer-label">MCP：</span>
            <span class="ct-turn-observer-value ct-turn-observer-status-pill ct-clickable" id="ct-status-pill" title="点击配置: ${state?.bridgeMessage || statusLabel}">
              <span class="ct-turn-observer-status-dot ${statusClass}"></span>
              <span>${statusLabel}</span>
            </span>
          </div>
        </div>
      `;
    }

    this.bindEvents();
  }

  private async openSettings() {
    this.openInlineSettingsModal();
  }

  private async openInlineSettingsModal() {
    if (this.modalContainer) {
      this.closeInlineSettingsModal();
    }

    const current = await loadSettings();

    this.modalContainer = document.createElement('div');
    this.modalContainer.className = 'ct-modal-root';
    this.modalContainer.innerHTML = `
      <div class="ct-modal-backdrop" id="ct-modal-backdrop">
        <div class="ct-modal-card">
          <div class="ct-modal-header">
            <div class="ct-modal-title">
              <span class="ct-turn-observer-logo-dot"></span>
              <span>Coding Tools 桥接设置</span>
            </div>
            <button type="button" class="ct-modal-close-btn" id="ct-modal-close" title="关闭">✕</button>
          </div>
          <div class="ct-modal-body">
            <div class="ct-form-group">
              <label class="ct-form-label">上报模式 (Bridge Mode)</label>
              <div class="ct-radio-group">
                <label class="ct-radio-label">
                  <input type="radio" name="ct-modal-mode" value="auto" ${current.bridgeMode === 'auto' ? 'checked' : ''}>
                  <span>Auto (自动探测，推荐)</span>
                </label>
                <label class="ct-radio-label">
                  <input type="radio" name="ct-modal-mode" value="local" ${current.bridgeMode === 'local' ? 'checked' : ''}>
                  <span>Local (仅本地)</span>
                </label>
                <label class="ct-radio-label">
                  <input type="radio" name="ct-modal-mode" value="remote" ${current.bridgeMode === 'remote' ? 'checked' : ''}>
                  <span>Remote (仅远端公网)</span>
                </label>
              </div>
            </div>

            <div class="ct-form-group">
              <label class="ct-form-label" for="ct-modal-local-url">Local Base URL</label>
              <input type="text" class="ct-input" id="ct-modal-local-url" value="${current.localBaseUrl}" placeholder="http://127.0.0.1:40111" />
            </div>

            <div class="ct-form-group">
              <label class="ct-form-label" for="ct-modal-remote-url">Remote Base URL</label>
              <input type="text" class="ct-input" id="ct-modal-remote-url" value="${current.remoteBaseUrl}" placeholder="https://mcp-myws.example.com" />
            </div>

            <div class="ct-form-group">
              <label class="ct-form-label" for="ct-modal-token">Browser Bridge Token (必填)</label>
              <div class="ct-input-password-wrap">
                <input type="password" class="ct-input" id="ct-modal-token" value="${current.bridgeToken}" placeholder="在 Desktop 桌面端设置 -> 共享密钥中复制" />
                <button type="button" class="ct-eye-btn" id="ct-modal-toggle-token" title="显示/隐藏 Token">👁️</button>
              </div>
              <span class="ct-form-hint">在 Coding Tools 桌面端“设置 -> 共享密钥 -> ChatGPT Observer 桥接密钥”中复制</span>
            </div>

            <div class="ct-modal-actions">
              <button type="button" class="ct-btn ct-btn-secondary" id="ct-modal-test-btn">测试连接</button>
              <button type="button" class="ct-btn ct-btn-primary" id="ct-modal-save-btn">保存配置</button>
            </div>

            <div class="ct-test-status" id="ct-modal-status"></div>
          </div>
        </div>
      </div>
    `;

    document.body.appendChild(this.modalContainer);
    this.bindModalEvents();
  }

  private closeInlineSettingsModal() {
    if (this.modalContainer) {
      this.modalContainer.remove();
      this.modalContainer = null;
    }
  }

  private bindModalEvents() {
    if (!this.modalContainer) return;

    const backdrop = this.modalContainer.querySelector('#ct-modal-backdrop');
    const closeBtn = this.modalContainer.querySelector('#ct-modal-close');
    const toggleTokenBtn = this.modalContainer.querySelector('#ct-modal-toggle-token');
    const tokenInput = this.modalContainer.querySelector('#ct-modal-token') as HTMLInputElement | null;
    const testBtn = this.modalContainer.querySelector('#ct-modal-test-btn') as HTMLButtonElement | null;
    const saveBtn = this.modalContainer.querySelector('#ct-modal-save-btn') as HTMLButtonElement | null;
    const statusEl = this.modalContainer.querySelector('#ct-modal-status') as HTMLDivElement | null;

    backdrop?.addEventListener('click', (e) => {
      if (e.target === backdrop) {
        this.closeInlineSettingsModal();
      }
    });

    closeBtn?.addEventListener('click', () => {
      this.closeInlineSettingsModal();
    });

    toggleTokenBtn?.addEventListener('click', () => {
      if (!tokenInput) return;
      tokenInput.type = tokenInput.type === 'password' ? 'text' : 'password';
    });

    const getFormData = () => {
      const modeRadio = this.modalContainer?.querySelector('input[name="ct-modal-mode"]:checked') as HTMLInputElement | null;
      const mode = (modeRadio?.value || 'auto') as BridgeMode;
      const localUrl = (this.modalContainer?.querySelector('#ct-modal-local-url') as HTMLInputElement)?.value?.trim() || 'http://127.0.0.1:40111';
      const remoteUrl = (this.modalContainer?.querySelector('#ct-modal-remote-url') as HTMLInputElement)?.value?.trim() || '';
      const token = (this.modalContainer?.querySelector('#ct-modal-token') as HTMLInputElement)?.value?.trim() || '';
      return { mode, localUrl, remoteUrl, token };
    };

    testBtn?.addEventListener('click', async () => {
      if (!statusEl) return;
      const { mode, localUrl, remoteUrl, token } = getFormData();
      if (!token) {
        statusEl.className = 'ct-test-status error';
        statusEl.textContent = '❌ 请先填写 Browser Bridge Token';
        return;
      }

      statusEl.className = 'ct-test-status info';
      statusEl.textContent = '⏳ 正在测试连接…';

      const endpointsToTry: string[] = [];
      if (mode === 'local') {
        endpointsToTry.push(localUrl);
      } else if (mode === 'remote') {
        if (!remoteUrl) {
          statusEl.className = 'ct-test-status error';
          statusEl.textContent = '❌ 远程模式下必须填写 Remote Base URL';
          return;
        }
        endpointsToTry.push(remoteUrl);
      } else {
        endpointsToTry.push(localUrl);
        if (remoteUrl) endpointsToTry.push(remoteUrl);
      }

      let success = false;
      let lastErr = '';

      for (const base of endpointsToTry) {
        const cleanBase = base.replace(/\/+$/, '');
        const target = `${cleanBase}/internal/chatgpt-turn-observer/status`;
        try {
          const resp = await fetch(target, {
            method: 'GET',
            headers: {
              'Authorization': `Bearer ${token}`,
            },
          });
          if (resp.ok) {
            const data = await resp.json();
            statusEl.className = 'ct-test-status success';
            statusEl.textContent = `✅ 连接成功: ${cleanBase} (v${data.version || '0.1.30'}, 工作区: ${data.workspace_id || 'default'})`;
            success = true;
            break;
          } else {
            const txt = await resp.text();
            lastErr = `${cleanBase} 响应 HTTP ${resp.status}: ${txt}`;
          }
        } catch (e: any) {
          lastErr = `${cleanBase} 请求失败: ${e.message || String(e)}`;
        }
      }

      if (!success) {
        statusEl.className = 'ct-test-status error';
        statusEl.textContent = `❌ 测试连接失败: ${lastErr}`;
      }
    });

    saveBtn?.addEventListener('click', async () => {
      if (!statusEl) return;
      const { mode, localUrl, remoteUrl, token } = getFormData();
      saveBtn.disabled = true;

      try {
        await saveSettings({
          bridgeMode: mode,
          localBaseUrl: localUrl,
          remoteBaseUrl: remoteUrl,
          bridgeToken: token,
        });

        statusEl.className = 'ct-test-status success';
        statusEl.textContent = '✅ 配置保存成功！';

        this.onSettingsSaved?.();

        setTimeout(() => {
          this.closeInlineSettingsModal();
        }, 600);
      } catch (e: any) {
        statusEl.className = 'ct-test-status error';
        statusEl.textContent = `❌ 保存失败: ${e.message || String(e)}`;
        saveBtn.disabled = false;
      }
    });
  }

  private bindEvents() {
    if (!this.container) return;

    const dragHandle = this.container.querySelector('#ct-drag-handle') as HTMLElement | null;
    if (dragHandle) {
      this.initDraggable(dragHandle);
    }

    const settingsBtn = this.container.querySelector('#ct-settings-btn');
    if (settingsBtn) {
      settingsBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.openSettings();
      });
    }

    const statusPill = this.container.querySelector('#ct-status-pill');
    if (statusPill) {
      statusPill.addEventListener('click', (e) => {
        e.stopPropagation();
        this.openSettings();
      });
    }

    const collapseBtn = this.container.querySelector('#ct-collapse-btn');
    if (collapseBtn) {
      collapseBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.isCollapsed = true;
        this.clampPosition();
        this.updatePositionStyle();
        this.render();
        this.onCollapsedChange?.(true);
      });
    }

    const expandBtn = this.container.querySelector('#ct-expand-btn');
    if (expandBtn) {
      expandBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.isCollapsed = false;
        this.clampPosition();
        this.updatePositionStyle();
        this.render();
        this.onCollapsedChange?.(false);
      });
    }
  }

  private initDraggable(handle: HTMLElement) {
    let startX = 0;
    let startY = 0;
    let initialPosX = 0;
    let initialPosY = 0;
    let isDragging = false;

    const onPointerDown = (e: PointerEvent) => {
      // 仅允许主键拖动，且排除按钮
      if (e.button !== 0 || (e.target as HTMLElement).tagName === 'BUTTON') return;
      isDragging = true;
      startX = e.clientX;
      startY = e.clientY;
      initialPosX = this.position.x;
      initialPosY = this.position.y;

      handle.setPointerCapture(e.pointerId);
      handle.addEventListener('pointermove', onPointerMove);
      handle.addEventListener('pointerup', onPointerUp);
      handle.addEventListener('pointercancel', onPointerUp);
    };

    const onPointerMove = (e: PointerEvent) => {
      if (!isDragging) return;
      const dx = e.clientX - startX;
      const dy = e.clientY - startY;
      this.position.x = initialPosX + dx;
      this.position.y = initialPosY + dy;
      this.clampPosition();
      this.updatePositionStyle();
    };

    const onPointerUp = (e: PointerEvent) => {
      if (!isDragging) return;
      isDragging = false;
      try {
        handle.releasePointerCapture(e.pointerId);
      } catch {
        // 忽略
      }
      handle.removeEventListener('pointermove', onPointerMove);
      handle.removeEventListener('pointerup', onPointerUp);
      handle.removeEventListener('pointercancel', onPointerUp);

      this.onPositionChange?.(this.position);
    };

    handle.addEventListener('pointerdown', onPointerDown);
  }

  public destroy() {
    this.stopTimerLoop();
    window.removeEventListener('resize', this.handleResize);
    if (this.container) {
      this.container.remove();
      this.container = null;
    }
  }
}
