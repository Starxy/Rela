import { invoke, isTauri } from '@tauri-apps/api/core';
import type { RelaService } from '../types';

export const desktopService: RelaService = {
  mode: 'desktop',
  getStatus: () => invoke('get_status'),
  connect: () => invoke('connect'),
  disconnect: () => invoke('disconnect'),
  reconnect: () => invoke('reconnect'),
  getResources: () => invoke('get_resources'),
  openResource: (id) => invoke('open_resource', { id }),
  runDiagnostics: () => invoke('run_diagnostics'),
  exportLogs: () => invoke('export_logs'),
  getVersion: () => invoke('get_version'),
  getPreferences: () => invoke('get_preferences'),
  savePreferences: (preferences) => invoke('save_preferences', { preferences }),
};

export async function createService(): Promise<RelaService> {
  // 桌面端永远走 Native；通信失败不能回退为虚假的连接成功。
  if (isTauri()) return desktopService;
  if (import.meta.env.DEV || import.meta.env.VITE_RELA_PREVIEW === 'true') {
    const { createPreviewService } = await import('./preview');
    let storage: Storage | undefined;
    try {
      storage = window.localStorage;
    } catch {
      /* 隐私模式可能禁用存储。 */
    }
    return createPreviewService(storage);
  }
  throw new Error('请在 Rela 桌面客户端中打开此应用。');
}
