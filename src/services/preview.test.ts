import { describe, expect, it } from 'vitest';
import { createPreviewService } from './preview';

describe('浏览器演示状态', () => {
  it('只有连接后才有虚拟地址和可用资源，断开后清理状态', async () => {
    const service = createPreviewService();
    expect(await service.getStatus()).toMatchObject({
      connected: false,
      core: 'stopped',
      virtual_ip: null,
      resources_available: 0,
    });
    expect(
      (await service.getResources()).every(
        (resource) => resource.availability === 'unknown',
      ),
    ).toBe(true);
    const connected = await service.connect();
    expect(connected.core).toBe('running');
    const resources = await service.getResources();
    expect(connected.virtual_ip).toBeTruthy();
    expect(connected.resources_available).toBe(
      resources.filter((resource) => resource.availability === 'reachable')
        .length,
    );
    expect(
      (await service.runDiagnostics()).checks.some(
        (check) => check.level === 'warning',
      ),
    ).toBe(true);
    await service.disconnect();
    expect(await service.getStatus()).toMatchObject({
      connected: false,
      core: 'stopped',
      virtual_ip: null,
      connection_type: null,
      latency_ms: null,
      gateway: 'unknown',
    });
  });

  it('独立演示实例不会共享连接状态', async () => {
    const first = createPreviewService();
    const second = createPreviewService();
    await first.connect();
    expect((await second.getStatus()).connected).toBe(false);
  });

  it('不打开示例 SSH 或 Web 地址', async () => {
    const service = createPreviewService();
    await service.connect();
    await expect(service.openResource('gpu01')).rejects.toThrow('资源暂不可用');
    await expect(service.openResource('missing')).rejects.toThrow('资源不存在');
  });

  it.each([
    'invalid json',
    '{"device_name":4}',
    '{"device_name":"","launch_at_login":false,"auto_connect":false}',
  ])('损坏的设置 %s 会回到默认值', async (saved) => {
    const service = createPreviewService({
      getItem: () => saved,
      setItem: () => {},
    });
    expect((await service.getPreferences()).device_name).toBe('我的电脑');
  });

  it('保存并重新读取偏好，拒绝空名称', async () => {
    let saved: string | null = null;
    const storage = {
      getItem: () => saved,
      setItem: (_key: string, value: string) => {
        saved = value;
      },
    };
    const service = createPreviewService(storage);
    await service.savePreferences({
      device_name: '  实验电脑  ',
      launch_at_login: true,
      auto_connect: false,
    });
    expect(
      (await createPreviewService(storage).getPreferences()).device_name,
    ).toBe('实验电脑');
    await expect(
      service.savePreferences({
        device_name: ' ',
        launch_at_login: false,
        auto_connect: false,
      }),
    ).rejects.toThrow();
  });

  it('存储失败不报告保存成功，也不覆盖当前偏好', async () => {
    const service = createPreviewService({
      getItem: () => null,
      setItem: () => {
        throw new Error('存储不可用');
      },
    });
    await expect(
      service.savePreferences({
        device_name: 'new name',
        launch_at_login: false,
        auto_connect: false,
      }),
    ).rejects.toThrow('存储不可用');
    expect((await service.getPreferences()).device_name).toBe('我的电脑');
  });
});
