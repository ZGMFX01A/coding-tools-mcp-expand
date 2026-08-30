import { DEFAULT_SETTINGS, type ObserverSettings } from './types';

const SETTINGS_KEY = 'ct_observer_settings';
const OBSERVER_ID_KEY = 'ct_observer_id';

function generateUuid(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

export async function getOrCreateObserverId(): Promise<string> {
  return new Promise((resolve) => {
    chrome.storage.local.get([OBSERVER_ID_KEY], (res) => {
      let id = res[OBSERVER_ID_KEY];
      if (!id || typeof id !== 'string') {
        id = generateUuid();
        chrome.storage.local.set({ [OBSERVER_ID_KEY]: id });
      }
      resolve(id);
    });
  });
}

export async function loadSettings(): Promise<ObserverSettings> {
  return new Promise((resolve) => {
    chrome.storage.local.get([SETTINGS_KEY], (res) => {
      const stored = res[SETTINGS_KEY];
      if (!stored || typeof stored !== 'object') {
        resolve({ ...DEFAULT_SETTINGS });
        return;
      }

      // Schema 迁移逻辑
      const settings: ObserverSettings = {
        schemaVersion: stored.schemaVersion || 1,
        bridgeMode: stored.bridgeMode || DEFAULT_SETTINGS.bridgeMode,
        localBaseUrl: stored.localBaseUrl || DEFAULT_SETTINGS.localBaseUrl,
        remoteBaseUrl: stored.remoteBaseUrl || DEFAULT_SETTINGS.remoteBaseUrl,
        bridgeToken: stored.bridgeToken || DEFAULT_SETTINGS.bridgeToken,
        overlayPosition: stored.overlayPosition || DEFAULT_SETTINGS.overlayPosition,
        overlayCollapsed: Boolean(stored.overlayCollapsed),
      };

      resolve(settings);
    });
  });
}

export async function saveSettings(updates: Partial<ObserverSettings>): Promise<ObserverSettings> {
  const current = await loadSettings();
  const next: ObserverSettings = { ...current, ...updates };
  return new Promise((resolve) => {
    chrome.storage.local.set({ [SETTINGS_KEY]: next }, () => {
      resolve(next);
    });
  });
}
