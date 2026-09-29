import { describe, expect, it } from 'vitest';
import { networkConfigUpdate } from './network-config';
import { createPreviewService } from './preview';
import { coreIsActive, type NetworkConfig } from '../types';

const config: NetworkConfig = {
  network_name: 'starxy',
  has_network_secret: true,
  peers: ['tcp://localhost:11010'],
  private_mode: true,
  disable_p2p: true,
  gateway_ip: null,
};

describe('网络配置', () => {
  it('空密钥表示保持原值，去重节点并允许关闭默认开关', () => {
    const update = networkConfigUpdate(
      { ...config, private_mode: false, disable_p2p: false },
      '',
      ' tcp://localhost:11010\n\ntcp://localhost:11010 ',
    );
    expect(update).not.toHaveProperty('network_secret');
    expect(update.peers).toEqual(['tcp://localhost:11010']);
    expect(update.private_mode).toBe(false);
    expect(update.disable_p2p).toBe(false);
  });
  it.each([
    'file:///C:/config',
    'tcp://user:password@host:11010',
    'tcp://localhost',
    'tcp://localhost:0',
    'tcp://localhost:11010?secret=value',
  ])('拒绝无效节点 %s', (peer) => {
    expect(() => networkConfigUpdate(config, '', peer)).toThrow();
  });
  it('没有已存密钥时必须填写；错误网关不会保存', () => {
    expect(() =>
      networkConfigUpdate(
        { ...config, has_network_secret: false },
        '',
        config.peers.join('\n'),
      ),
    ).toThrow('密钥');
    expect(() =>
      networkConfigUpdate(
        { ...config, gateway_ip: '999.1.1.1' },
        '',
        config.peers.join('\n'),
      ),
    ).toThrow('网关');
  });
  it('演示保存和重载配置，密钥不会进入浏览器存储或返回对象', async () => {
    const saved = new Map<string, string>();
    const storage = {
      getItem: (key: string) => saved.get(key) ?? null,
      setItem: (key: string, value: string) => {
        saved.set(key, value);
      },
    };
    const service = createPreviewService(storage);
    const updated = await service.saveNetworkConfig({
      network_name: 'other',
      network_secret: 'test-private-secret',
      peers: ['udp://localhost:11010'],
      private_mode: false,
      disable_p2p: false,
      gateway_ip: '10.1.2.3',
    });
    expect(JSON.stringify(updated)).not.toContain('test-private-secret');
    expect([...saved.values()].join('')).not.toContain('test-private-secret');
    expect(await createPreviewService(storage).getNetworkConfig()).toEqual(
      updated,
    );
    expect(await service.resetNetworkConfig()).toMatchObject({
      network_name: 'starxy',
      private_mode: true,
      disable_p2p: true,
    });
  });
  it('引擎已启动但尚未连通时仍允许断开', async () => {
    const status = await createPreviewService().getStatus();
    expect(coreIsActive({ ...status, connected: false, core: 'running' })).toBe(
      true,
    );
    expect(coreIsActive({ ...status, core: 'stopped' })).toBe(false);
  });
});
