import { formatModelDisplayName } from './parsers';
import { loadSettings, saveSettings } from './settings';
import { DEFAULT_LOCAL_PORT, type BridgeMode, type ObserverSettings, type TabTurnState } from './types';

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
    this.container.replaceChildren();

    const actualModelText = state?.actualModel
      ? formatModelDisplayName(state.actualModel)
      : null;
    const requestedModelText = state?.requestedModel
      ? formatModelDisplayName(state.requestedModel)
      : null;

    const requestedRowValue = requestedModelText || '—';
    const responseRowValue = actualModelText
      ? actualModelText
      : (state?.state === 'active' || state?.state === 'turn_starting')
      ? '检测中…'
      : '—';

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
      const miniWrap = document.createElement('div');
      miniWrap.className = 'ct-turn-observer-mini';
      miniWrap.id = 'ct-drag-handle';

      const dot = document.createElement('span');
      dot.className = 'ct-turn-observer-logo-dot';
      miniWrap.appendChild(dot);

      const titleSpan = document.createElement('span');
      titleSpan.textContent = 'CT';
      miniWrap.appendChild(titleSpan);

      const sep1 = document.createElement('span');
      sep1.textContent = '·';
      miniWrap.appendChild(sep1);

      const modelSpan = document.createElement('span');
      const miniModelText = requestedModelText
        ? actualModelText
          ? `${requestedModelText} → ${actualModelText}`
          : requestedModelText
        : actualModelText || '—';
      modelSpan.textContent = miniModelText;
      miniWrap.appendChild(modelSpan);

      const sep2 = document.createElement('span');
      sep2.textContent = '·';
      miniWrap.appendChild(sep2);

      const timerSpan = document.createElement('span');
      timerSpan.className = 'ct-mini-timer';
      timerSpan.textContent = durationText;
      miniWrap.appendChild(timerSpan);

      const statusDot = document.createElement('span');
      statusDot.className = `ct-turn-observer-status-dot ${statusClass}`;
      miniWrap.appendChild(statusDot);

      const expandBtn = document.createElement('button');
      expandBtn.type = 'button';
      expandBtn.className = 'ct-turn-observer-toggle-btn';
      expandBtn.id = 'ct-expand-btn';
      expandBtn.title = '展开';
      expandBtn.textContent = '＋';
      miniWrap.appendChild(expandBtn);

      this.container.appendChild(miniWrap);
    } else {
      const card = document.createElement('div');
      card.className = 'ct-turn-observer-card';

      // Header
      const header = document.createElement('div');
      header.className = 'ct-turn-observer-header';
      header.id = 'ct-drag-handle';

      const titleWrap = document.createElement('div');
      titleWrap.className = 'ct-turn-observer-title';
      const logoDot = document.createElement('span');
      logoDot.className = 'ct-turn-observer-logo-dot';
      const titleText = document.createElement('span');
      titleText.textContent = 'Coding Tools';
      titleWrap.appendChild(logoDot);
      titleWrap.appendChild(titleText);
      header.appendChild(titleWrap);

      const actions = document.createElement('div');
      actions.className = 'ct-turn-observer-actions';

      const settingsBtn = document.createElement('button');
      settingsBtn.type = 'button';
      settingsBtn.className = 'ct-turn-observer-toggle-btn';
      settingsBtn.id = 'ct-settings-btn';
      settingsBtn.title = '配置 (打开设置)';
      settingsBtn.textContent = '⚙';
      actions.appendChild(settingsBtn);

      const collapseBtn = document.createElement('button');
      collapseBtn.type = 'button';
      collapseBtn.className = 'ct-turn-observer-toggle-btn';
      collapseBtn.id = 'ct-collapse-btn';
      collapseBtn.title = '折叠';
      collapseBtn.textContent = '－';
      actions.appendChild(collapseBtn);

      header.appendChild(actions);
      card.appendChild(header);

      // Row 1: 请求模型
      const row1 = this.createRow('请求模型：', requestedRowValue);
      card.appendChild(row1);

      // Row 2: 响应模型
      const row2 = this.createRow('响应模型：', responseRowValue);
      card.appendChild(row2);

      // Row 3: 本轮耗时
      const row3 = document.createElement('div');
      row3.className = 'ct-turn-observer-row';
      const r3Label = document.createElement('span');
      r3Label.className = 'ct-turn-observer-label';
      r3Label.textContent = '本轮：';
      const r3Val = document.createElement('span');
      r3Val.className = 'ct-turn-observer-value ct-timer-val';
      r3Val.textContent = durationText;
      row3.appendChild(r3Label);
      row3.appendChild(r3Val);
      card.appendChild(row3);

      // Row 4: MCP 状态
      const row4 = document.createElement('div');
      row4.className = 'ct-turn-observer-row';
      const r4Label = document.createElement('span');
      r4Label.className = 'ct-turn-observer-label';
      r4Label.textContent = 'MCP：';

      const pill = document.createElement('span');
      pill.className = 'ct-turn-observer-value ct-turn-observer-status-pill ct-clickable';
      pill.id = 'ct-status-pill';
      pill.title = `点击配置: ${state?.bridgeMessage || statusLabel}`;

      const pDot = document.createElement('span');
      pDot.className = `ct-turn-observer-status-dot ${statusClass}`;
      const pText = document.createElement('span');
      pText.textContent = statusLabel;

      pill.appendChild(pDot);
      pill.appendChild(pText);
      row4.appendChild(r4Label);
      row4.appendChild(pill);
      card.appendChild(row4);

      this.container.appendChild(card);
    }

    this.bindEvents();
  }

  private createRow(label: string, value: string): HTMLDivElement {
    const row = document.createElement('div');
    row.className = 'ct-turn-observer-row';
    const l = document.createElement('span');
    l.className = 'ct-turn-observer-label';
    l.textContent = label;
    const v = document.createElement('span');
    v.className = 'ct-turn-observer-value';
    v.title = value;
    v.textContent = value;
    row.appendChild(l);
    row.appendChild(v);
    return row;
  }

  private openSettings() {
    // 设置入口必须在当前页面可用。Edge 可能拦截 chrome-extension:// 的
    // Options 页面，因此不能把它作为唯一入口。
    this.openInlineSettingsModal();
  }

  private openExtensionOptionsPage() {
    const runtime = typeof chrome !== 'undefined' ? chrome.runtime : undefined;
    const optionsUrl = runtime?.getURL?.('options/options.html');

    if (runtime?.openOptionsPage) {
      try {
        const result = runtime.openOptionsPage();
        // openOptionsPage 在不同浏览器版本中可能返回 void 或 Promise。
        // Promise 被拒绝时再尝试直接打开正确的扩展资源路径。
        if (result && typeof (result as Promise<void>).catch === 'function') {
          (result as Promise<void>).catch(() => {
            if (optionsUrl) window.open(optionsUrl, '_blank', 'noopener,noreferrer');
          });
        }
        return;
      } catch {
        // 继续走直接 URL 备用路径。
      }
    }

    if (optionsUrl) {
      window.open(optionsUrl, '_blank', 'noopener,noreferrer');
    }
  }

  private async openInlineSettingsModal() {
    if (this.modalContainer) {
      this.closeInlineSettingsModal();
    }

    const current = await loadSettings();

    this.modalContainer = document.createElement('div');
    this.modalContainer.className = 'ct-modal-root';

    const backdrop = document.createElement('div');
    backdrop.className = 'ct-modal-backdrop';
    backdrop.id = 'ct-modal-backdrop';

    const card = document.createElement('div');
    card.className = 'ct-modal-card';

    // Header
    const header = document.createElement('div');
    header.className = 'ct-modal-header';
    const titleWrap = document.createElement('div');
    titleWrap.className = 'ct-modal-title';
    const dot = document.createElement('span');
    dot.className = 'ct-turn-observer-logo-dot';
    const titleText = document.createElement('span');
    titleText.textContent = 'Coding Tools 桥接设置';
    titleWrap.appendChild(dot);
    titleWrap.appendChild(titleText);
    header.appendChild(titleWrap);

    const closeBtn = document.createElement('button');
    closeBtn.type = 'button';
    closeBtn.className = 'ct-modal-close-btn';
    closeBtn.id = 'ct-modal-close';
    closeBtn.title = '关闭';
    closeBtn.textContent = '✕';
    header.appendChild(closeBtn);
    card.appendChild(header);

    // Body
    const body = document.createElement('div');
    body.className = 'ct-modal-body';

    // Bridge Mode
    const fgMode = document.createElement('div');
    fgMode.className = 'ct-form-group';
    const lblMode = document.createElement('label');
    lblMode.className = 'ct-form-label';
    lblMode.textContent = '上报模式 (Bridge Mode)';
    fgMode.appendChild(lblMode);

    const radioGroup = document.createElement('div');
    radioGroup.className = 'ct-radio-group';
    const modes: Array<{ val: BridgeMode; label: string }> = [
      { val: 'auto', label: 'Auto (自动探测，推荐)' },
      { val: 'local', label: 'Local (仅本地)' },
      { val: 'remote', label: 'Remote (仅远端公网)' },
    ];
    for (const m of modes) {
      const rl = document.createElement('label');
      rl.className = 'ct-radio-label';
      const input = document.createElement('input');
      input.type = 'radio';
      input.name = 'ct-modal-mode';
      input.value = m.val;
      if (current.bridgeMode === m.val) input.checked = true;
      const span = document.createElement('span');
      span.textContent = m.label;
      rl.appendChild(input);
      rl.appendChild(span);
      radioGroup.appendChild(rl);
    }
    fgMode.appendChild(radioGroup);
    body.appendChild(fgMode);

    // Local Base URL
    const fgLocal = document.createElement('div');
    fgLocal.className = 'ct-form-group';
    const lblLocal = document.createElement('label');
    lblLocal.className = 'ct-form-label';
    lblLocal.htmlFor = 'ct-modal-local-url';
    lblLocal.textContent = 'Local Base URL';
    const inputLocal = document.createElement('input');
    inputLocal.type = 'text';
    inputLocal.className = 'ct-input';
    inputLocal.id = 'ct-modal-local-url';
    inputLocal.value = current.localBaseUrl;
    inputLocal.placeholder = `http://127.0.0.1:${DEFAULT_LOCAL_PORT}`;
    fgLocal.appendChild(lblLocal);
    fgLocal.appendChild(inputLocal);
    body.appendChild(fgLocal);

    // Remote Base URL
    const fgRemote = document.createElement('div');
    fgRemote.className = 'ct-form-group';
    const lblRemote = document.createElement('label');
    lblRemote.className = 'ct-form-label';
    lblRemote.htmlFor = 'ct-modal-remote-url';
    lblRemote.textContent = 'Remote Base URL';
    const inputRemote = document.createElement('input');
    inputRemote.type = 'text';
    inputRemote.className = 'ct-input';
    inputRemote.id = 'ct-modal-remote-url';
    inputRemote.value = current.remoteBaseUrl;
    inputRemote.placeholder = 'https://mcp-myws.example.com';
    fgRemote.appendChild(lblRemote);
    fgRemote.appendChild(inputRemote);
    body.appendChild(fgRemote);

    // Browser Bridge Token
    const fgToken = document.createElement('div');
    fgToken.className = 'ct-form-group';
    const lblToken = document.createElement('label');
    lblToken.className = 'ct-form-label';
    lblToken.htmlFor = 'ct-modal-token';
    lblToken.textContent = 'Browser Bridge Token';

    const tokenWrap = document.createElement('div');
    tokenWrap.className = 'ct-input-password-wrap';
    const inputToken = document.createElement('input');
    inputToken.type = 'password';
    inputToken.className = 'ct-input';
    inputToken.id = 'ct-modal-token';
    inputToken.autocomplete = 'off';
    inputToken.value = current.bridgeToken;
    inputToken.placeholder = '从 Coding Tools Desktop 设置中复制';

    const toggleTokenBtn = document.createElement('button');
    toggleTokenBtn.type = 'button';
    toggleTokenBtn.className = 'ct-eye-btn';
    toggleTokenBtn.id = 'ct-modal-toggle-token';
    toggleTokenBtn.title = '显示/隐藏 Token';
    toggleTokenBtn.textContent = '显示';
    tokenWrap.appendChild(inputToken);
    tokenWrap.appendChild(toggleTokenBtn);
    fgToken.appendChild(lblToken);
    fgToken.appendChild(tokenWrap);

    const tokenHint = document.createElement('span');
    tokenHint.className = 'ct-form-hint';
    tokenHint.textContent = '密钥不会显示在页面文字中，保存后写入扩展本地存储。';
    fgToken.appendChild(tokenHint);
    body.appendChild(fgToken);

    // 独立选项页作为辅助入口；当前页无法打开时仍可直接在上面的密码框配置。
    const fgSecurity = document.createElement('div');
    fgSecurity.className = 'ct-form-group';
    const securityTip = document.createElement('div');
    securityTip.className = 'ct-form-hint';
    securityTip.style.color = '#38bdf8';
    securityTip.textContent = '需要完整配置时，也可以打开扩展独立选项页。';
    fgSecurity.appendChild(securityTip);
    body.appendChild(fgSecurity);

    // Actions
    const actionsWrap = document.createElement('div');
    actionsWrap.className = 'ct-modal-actions';
    const openOptionsBtn = document.createElement('button');
    openOptionsBtn.type = 'button';
    openOptionsBtn.className = 'ct-btn ct-btn-secondary';
    openOptionsBtn.id = 'ct-modal-open-options';
    openOptionsBtn.textContent = '⚙️ 打开独立选项页';

    const saveBtn = document.createElement('button');
    saveBtn.type = 'button';
    saveBtn.className = 'ct-btn ct-btn-primary';
    saveBtn.id = 'ct-modal-save-btn';
    saveBtn.textContent = '保存配置';

    actionsWrap.appendChild(openOptionsBtn);
    actionsWrap.appendChild(saveBtn);
    body.appendChild(actionsWrap);

    const statusEl = document.createElement('div');
    statusEl.className = 'ct-test-status';
    statusEl.id = 'ct-modal-status';
    body.appendChild(statusEl);

    card.appendChild(body);
    backdrop.appendChild(card);
    this.modalContainer.appendChild(backdrop);
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
    const openOptionsBtn = this.modalContainer.querySelector('#ct-modal-open-options') as HTMLButtonElement | null;
    const saveBtn = this.modalContainer.querySelector('#ct-modal-save-btn') as HTMLButtonElement | null;
    const statusEl = this.modalContainer.querySelector('#ct-modal-status') as HTMLDivElement | null;
    const tokenInput = this.modalContainer.querySelector('#ct-modal-token') as HTMLInputElement | null;
    const toggleTokenBtn = this.modalContainer.querySelector('#ct-modal-toggle-token') as HTMLButtonElement | null;

    backdrop?.addEventListener('click', (e) => {
      if (e.target === backdrop) {
        this.closeInlineSettingsModal();
      }
    });

    closeBtn?.addEventListener('click', () => {
      this.closeInlineSettingsModal();
    });

    openOptionsBtn?.addEventListener('click', () => {
      this.openExtensionOptionsPage();
    });

    toggleTokenBtn?.addEventListener('click', () => {
      if (!tokenInput) return;
      const isVisible = tokenInput.type === 'text';
      tokenInput.type = isVisible ? 'password' : 'text';
      toggleTokenBtn.textContent = isVisible ? '显示' : '隐藏';
    });

    const getFormData = () => {
      const modeRadio = this.modalContainer?.querySelector('input[name="ct-modal-mode"]:checked') as HTMLInputElement | null;
      const mode = (modeRadio?.value || 'auto') as BridgeMode;
      const localUrl = (this.modalContainer?.querySelector('#ct-modal-local-url') as HTMLInputElement)?.value?.trim() || `http://127.0.0.1:${DEFAULT_LOCAL_PORT}`;
      const remoteUrl = (this.modalContainer?.querySelector('#ct-modal-remote-url') as HTMLInputElement)?.value?.trim() || '';
      const bridgeToken = (this.modalContainer?.querySelector('#ct-modal-token') as HTMLInputElement)?.value?.trim() || '';
      return { mode, localUrl, remoteUrl, bridgeToken };
    };

    saveBtn?.addEventListener('click', async () => {
      if (!statusEl) return;
      const { mode, localUrl, remoteUrl, bridgeToken } = getFormData();
      statusEl.className = 'ct-test-status info';
      statusEl.textContent = '💾 正在保存配置…';
      try {
        await saveSettings({
          bridgeMode: mode,
          localBaseUrl: localUrl,
          remoteBaseUrl: remoteUrl,
          bridgeToken,
        });
        statusEl.className = 'ct-test-status success';
        statusEl.textContent = '✅ 配置已保存！';
        await this.onSettingsSaved?.();
        setTimeout(() => {
          this.closeInlineSettingsModal();
        }, 800);
      } catch (err: unknown) {
        statusEl.className = 'ct-test-status error';
        statusEl.textContent = `❌ 保存失败: ${err instanceof Error ? err.message : String(err)}`;
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
