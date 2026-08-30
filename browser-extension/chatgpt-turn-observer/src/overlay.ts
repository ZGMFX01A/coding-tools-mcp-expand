import { formatModelDisplayName } from './parsers';
import type { TabTurnState } from './types';

export class TurnObserverOverlay {
  private container: HTMLDivElement | null = null;
  private isCollapsed = false;
  private position = { x: 0, y: 0 };
  private onPositionChange?: (pos: { x: number; y: number }) => void;
  private onCollapsedChange?: (collapsed: boolean) => void;

  // 计时器状态
  private timerInterval: number | null = null;
  private timerSeconds = 0;
  private isTimerRunning = false;

  constructor(
    initialPos: { x: number; y: number } | null,
    initialCollapsed: boolean,
    onPositionChange?: (pos: { x: number; y: number }) => void,
    onCollapsedChange?: (collapsed: boolean) => void
  ) {
    this.isCollapsed = initialCollapsed;
    this.onPositionChange = onPositionChange;
    this.onCollapsedChange = onCollapsedChange;

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
    let modelRowValue = '检测中…';
    if (actualModelText) {
      modelRowLabel = '模型：';
      modelRowValue = actualModelText;
    } else if (requestedModelText) {
      modelRowLabel = '请求：';
      modelRowValue = `${requestedModelText} (实际: 未知)`;
    }

    let statusClass = 'idle';
    let statusLabel = '未连接';
    if (state?.bridgeStatus === 'synced') {
      statusClass = 'synced';
      statusLabel = '已同步';
    } else if (state?.bridgeStatus === 'sending') {
      statusClass = 'sending';
      statusLabel = '发送中';
    } else if (state?.bridgeStatus === 'failed') {
      statusClass = 'failed';
      statusLabel = '同步失败';
    } else if (state?.bridgeStatus === 'connecting') {
      statusClass = 'sending';
      statusLabel = '连接中';
    }

    const durationText = this.formatDuration(this.timerSeconds);

    if (this.isCollapsed) {
      this.container.innerHTML = `
        <div class="ct-turn-observer-mini" id="ct-drag-handle">
          <span class="ct-turn-observer-logo-dot"></span>
          <span>CT</span>
          <span>·</span>
          <span>${actualModelText || requestedModelText || '检测中'}</span>
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
            <button type="button" class="ct-turn-observer-toggle-btn" id="ct-collapse-btn" title="折叠">－</button>
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
            <span class="ct-turn-observer-value ct-turn-observer-status-pill" title="${state?.bridgeMessage || statusLabel}">
              <span class="ct-turn-observer-status-dot ${statusClass}"></span>
              <span>${statusLabel}</span>
            </span>
          </div>
        </div>
      `;
    }

    this.bindEvents();
  }

  private bindEvents() {
    if (!this.container) return;

    const dragHandle = this.container.querySelector('#ct-drag-handle') as HTMLElement | null;
    if (dragHandle) {
      this.initDraggable(dragHandle);
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
