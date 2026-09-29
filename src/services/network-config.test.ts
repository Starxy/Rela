import { describe, expect, it } from 'vitest';
import { networkConfigUpdate } from './network-config';
import { createPreviewService } from './preview';
import { coreIsActive, type NetworkConfig } from '../types';

const config: NetworkConfig = {
  network_name: 'lab201',
  has_credential: true,
  peers: ['tcp://localhost:11010'],
  private_mode: true,
  disable_p2p: true,
  gateway_ip: null,
};
const credential = btoa('B'.repeat(32));

describe('网络配置', () => {
  it('空密钥表示保持原值，去重节点并允许关闭默认开关', () => {
    const update = networkConfigUpdate(
      { ...config, private_mode: false, disable_p2p: false },
      '',
      ' tcp://localhost:11010\n\ntcp://localhost:11010 ',
    );
    expect(update).not.toHaveProperty('credential_secret');
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
  it('允许先保存公开配置；错误凭据和错误网关不会保存', () => {
    expect(() =>
      networkConfigUpdate(
        { ...config, has_credential: false },
        '',
        config.peers.join('\n'),
      ),
    ).not.toThrow();
    expect(() =>
      networkConfigUpdate(config, 'password', config.peers.join('\n')),
    ).toThrow('credential');
    expect(() =>
      networkConfigUpdate(config, btoa('short'), config.peers.join('\n')),
    ).toThrow('credential');
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
      credential_secret: credential,
      peers: ['udp://localhost:11010'],
      private_mode: false,
      disable_p2p: false,
      gateway_ip: '10.1.2.3',
    });
    expect(JSON.stringify(updated)).not.toContain(credential);
    expect([...saved.values()].join('')).not.toContain(credential);
    expect(await createPreviewService(storage).getNetworkConfig()).toEqual(
      updated,
    );
    expect(await service.resetNetworkConfig()).toMatchObject({
      network_name: 'lab201',
      has_credential: false,
      private_mode: true,
      disable_p2p: true,
    });
  });
  it('更换网络必须更换或清除凭据，清除后拒绝连接', async () => {
    const service = createPreviewService();
    await expect(
      service.saveNetworkConfig({
        ...networkConfigUpdate(config, '', config.peers.join('\n')),
        network_name: 'other',
      }),
    ).rejects.toThrow('更换网络');
    const cleared = await service.saveNetworkConfig({
      ...networkConfigUpdate(config, '', config.peers.join('\n')),
      credential_secret: '',
    });
    expect(cleared.has_credential).toBe(false);
    await expect(service.connect()).rejects.toThrow('credential');
  });
  it('引擎已启动但尚未连通时仍允许断开', async () => {
    const status = await createPreviewService().getStatus();
    expect(coreIsActive({ ...status, connected: false, core: 'running' })).toBe(
      true,
    );
    expect(coreIsActive({ ...status, core: 'stopped' })).toBe(false);
  });
});
