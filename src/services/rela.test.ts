import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import { createService, desktopService } from './rela';

describe('桌面权限边界', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.unstubAllEnvs();
  });

  it('桌面端即使显式启用演示也只使用真实 Native 接口', async () => {
    vi.mocked(isTauri).mockReturnValue(true);
    vi.stubEnv('VITE_RELA_PREVIEW', 'true');
    expect(await createService()).toBe(desktopService);
  });

  it('Core 不可用时不会回退为演示连接成功', async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: 'core_unavailable',
      message: '网络引擎不可用',
    });
    await expect(desktopService.connect()).rejects.toMatchObject({
      code: 'core_unavailable',
    });
    expect(invoke).toHaveBeenCalledWith('connect');
  });

  it('资源入口只传递 ID', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await desktopService.openResource('gpu01');
    expect(invoke).toHaveBeenCalledWith('open_resource', { id: 'gpu01' });
  });

  it('发布产物在普通浏览器中不默认进入演示', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    vi.stubEnv('DEV', false);
    vi.stubEnv('VITE_RELA_PREVIEW', 'false');
    await expect(createService()).rejects.toThrow('桌面客户端');
  });
});
